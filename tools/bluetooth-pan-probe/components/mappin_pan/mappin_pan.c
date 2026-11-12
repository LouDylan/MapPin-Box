/* Read-only iPhone connectivity test over Bluetooth Classic PAN. */

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "driver/uart.h"
#include "esp_bt.h"
#include "esp_bt_device.h"
#include "esp_bt_main.h"
#include "esp_event.h"
#include "esp_gap_bt_api.h"
#include "esp_http_server.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"
#include "lwip/sockets.h"
#include "mdns.h"
#include "nvs_flash.h"
#include "pan_link.h"
#include "mappin_pan.h"

enum event_type { PAN_READY, SCAN_DONE, PAN_OPEN, PAN_CLOSE, PAIR_CONFIRM };

struct event {
    enum event_type type;
    esp_bd_addr_t address;
    uint16_t handle;
    uint32_t value;
};

static const char *TAG = "mappin_bt";
static QueueHandle_t events;
static TaskHandle_t probe_task;
static esp_netif_t *pan;
static bool mdns_ready;
static bool reconnect_bonded;
static bool retry_pending;
static TickType_t retry_at;
static mappin_pan_command_fn extra_command;

/* The console task owns connection and pairing state. */
static enum { STARTING, IDLE, SCANNING, CONNECTING, CONNECTED } phase;
static esp_bd_addr_t target;
static uint16_t handle = ESP_PAN_INVALID_HANDLE;
static bool confirmation_pending;
static uint32_t confirmation_code;

static void post(struct event event)
{
    if (xQueueSend(events, &event, 0) != pdTRUE)
        ESP_LOGW(TAG, "Bluetooth event queue full");
}

static void report_peer(esp_bt_gap_cb_param_t *event)
{
    const uint8_t *name = NULL;
    uint8_t length = 0;
    for (int i = 0; i < event->disc_res.num_prop; i++) {
        const esp_bt_gap_dev_prop_t *prop = &event->disc_res.prop[i];
        if (prop->type == ESP_BT_GAP_DEV_PROP_BDNAME && prop->len > 0) {
            name = prop->val;
            length = prop->len > ESP_BT_GAP_MAX_BDNAME_LEN ? ESP_BT_GAP_MAX_BDNAME_LEN : prop->len;
            break;
        }
        if (prop->type != ESP_BT_GAP_DEV_PROP_EIR)
            continue;
        name = esp_bt_gap_resolve_eir_data(prop->val, ESP_BT_EIR_TYPE_CMPL_LOCAL_NAME, &length);
        if (!name)
            name = esp_bt_gap_resolve_eir_data(prop->val, ESP_BT_EIR_TYPE_SHORT_LOCAL_NAME, &length);
    }
    if (!name || !length)
        return;
    char label[ESP_BT_GAP_MAX_BDNAME_LEN + 1];
    for (unsigned i = 0; i < length; i++)
        label[i] = (name[i] < 32 || name[i] == 127) ? ' ' : name[i];
    label[length] = '\0';
    const uint8_t *mac = event->disc_res.bda;
    ESP_LOGI(TAG, "MAPPIN_BT_FOUND %02x:%02x:%02x:%02x:%02x:%02x %s",
             mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], label);
}

static void gap_event(esp_bt_gap_cb_event_t type, esp_bt_gap_cb_param_t *data)
{
    switch (type) {
    case ESP_BT_GAP_DISC_RES_EVT:
        report_peer(data);
        break;
    case ESP_BT_GAP_DISC_STATE_CHANGED_EVT:
        if (data->disc_st_chg.state == ESP_BT_GAP_DISCOVERY_STOPPED)
            post((struct event){ .type = SCAN_DONE });
        break;
    case ESP_BT_GAP_CFM_REQ_EVT: {
        struct event event = { .type = PAIR_CONFIRM, .value = data->cfm_req.num_val };
        memcpy(event.address, data->cfm_req.bda, sizeof(event.address));
        post(event);
        break;
    }
    case ESP_BT_GAP_PIN_REQ_EVT:
        esp_bt_gap_pin_reply(data->pin_req.bda, false, 0, NULL);
        break;
    case ESP_BT_GAP_AUTH_CMPL_EVT:
        ESP_LOGI(TAG, "MAPPIN_BT_AUTH status=%d", data->auth_cmpl.stat);
        break;
    default:
        break;
    }
}

static void pan_event(esp_pan_cb_event_t type, esp_pan_cb_param_t *data)
{
    switch (type) {
    case ESP_PAN_SET_ROLE_EVT:
        post((struct event){ .type = PAN_READY, .value = data->set_role.status });
        break;
    case ESP_PAN_OPEN_EVT: {
        struct event event = {
            .type = PAN_OPEN, .handle = data->open.handle, .value = data->open.status,
        };
        memcpy(event.address, data->open.remote_bda, sizeof(event.address));
        post(event);
        break;
    }
    case ESP_PAN_CLOSE_EVT:
        post((struct event){ .type = PAN_CLOSE, .handle = data->close.handle });
        break;
    case ESP_PAN_DATA_IND_EVT:
        pan_link_receive(data);
        break;
    case ESP_PAN_CONG_EVT:
        pan_link_congestion(data->cong.handle, data->cong.cong);
        break;
    default:
        break;
    }
}

static int tcp_probe(uint32_t address, uint16_t port)
{
    int socket_fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (socket_fd < 0)
        return errno;
    const struct sockaddr_in peer = {
        .sin_family = AF_INET, .sin_port = htons(port), .sin_addr.s_addr = address,
    };
    int error = 0;
    if (fcntl(socket_fd, F_SETFL, O_NONBLOCK) != 0) {
        error = errno;
    } else if (connect(socket_fd, (const struct sockaddr *)&peer, sizeof(peer)) != 0) {
        error = errno;
        if (error == EINPROGRESS) {
            fd_set write_ready;
            FD_ZERO(&write_ready);
            FD_SET(socket_fd, &write_ready);
            struct timeval timeout = { .tv_sec = 3 };
            int ready = select(socket_fd + 1, NULL, &write_ready, NULL, &timeout);
            socklen_t size = sizeof(error);
            if (ready == 0)
                error = ETIMEDOUT;
            else if (ready < 0 || getsockopt(socket_fd, SOL_SOCKET, SO_ERROR, &error, &size) < 0)
                error = errno;
        }
    }
    close(socket_fd);
    return error;
}

static void discover(const char *service)
{
    mdns_result_t *results = NULL;
    esp_err_t status = mdns_query_ptr(service, "_tcp", 2500, 4, &results);
    for (const mdns_result_t *item = results; item; item = item->next) {
        ESP_LOGI(TAG, "MAPPIN_BT_MDNS %s port=%u", service, item->port);
        for (const mdns_ip_addr_t *addr = item->addr; addr; addr = addr->next) {
            if (addr->addr.type == ESP_IPADDR_TYPE_V4)
                ESP_LOGI(TAG, "MAPPIN_BT_MDNS_ADDRESS " IPSTR, IP2STR(&addr->addr.u_addr.ip4));
            else if (addr->addr.type == ESP_IPADDR_TYPE_V6)
                ESP_LOGI(TAG, "MAPPIN_BT_MDNS_ADDRESS " IPV6STR, IPV62STR(addr->addr.u_addr.ip6));
        }
    }
    if (!results)
        ESP_LOGI(TAG, "MAPPIN_BT_MDNS %s no results (%s)", service, esp_err_to_name(status));
    mdns_query_results_free(results);
}

static void check_peer(void *ctx)
{
    for (;;) {
        ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
        esp_netif_ip_info_t info;
        if (esp_netif_get_ip_info(pan, &info) != ESP_OK || !info.gw.addr)
            continue;
        const uint16_t ports[] = {49152, 62078};
        for (unsigned i = 0; i < sizeof(ports) / sizeof(ports[0]); i++) {
            int error = tcp_probe(info.gw.addr, ports[i]);
            ESP_LOGI(TAG, "MAPPIN_BT_TCP port=%u result=%s errno=%d",
                     ports[i], error ? strerror(error) : "open", error);
        }
        if (mdns_ready) {
            discover("_remotepairing");
            discover("_remoted");
        }
        ESP_LOGI(TAG, "MAPPIN_BT_CHECK_DONE");
    }
}

static void got_ip(void *ctx, esp_event_base_t base, int32_t id, void *data)
{
    const ip_event_got_ip_t *event = data;
    if (event->esp_netif != pan)
        return;
    ESP_LOGI(TAG, "MAPPIN_BT_IP http://" IPSTR "/ gateway=" IPSTR,
             IP2STR(&event->ip_info.ip), IP2STR(&event->ip_info.gw));
    if (mdns_ready)
        mdns_netif_action(pan, MDNS_EVENT_ENABLE_IP4 | MDNS_EVENT_ENABLE_IP6);
    xTaskNotifyGive(probe_task);
}

static esp_err_t home(httpd_req_t *request)
{
    httpd_resp_set_type(request, "text/plain; charset=utf-8");
    httpd_resp_set_hdr(request, "Cache-Control", "no-store");
    return httpd_resp_sendstr(request, "MapPin-Box经典蓝牙 PAN 已连接。此页面仅测试网络。\n");
}

static bool parse_address(const char *value, esp_bd_addr_t address)
{
    unsigned octets[6];
    char extra;
    if (strlen(value) != 17 ||
        sscanf(value, "%2x:%2x:%2x:%2x:%2x:%2x%c", &octets[0], &octets[1], &octets[2],
               &octets[3], &octets[4], &octets[5], &extra) != 6)
        return false;
    for (unsigned i = 0; i < 6; i++) {
        if (octets[i] > 255)
            return false;
        address[i] = octets[i];
    }
    return true;
}

static void schedule_retry(void)
{
    if (reconnect_bonded) {
        retry_pending = true;
        retry_at = xTaskGetTickCount() + pdMS_TO_TICKS(10000);
    }
}

static void connect_target(void)
{
    esp_err_t status = esp_pan_connect(target, ESP_PAN_ROLE_PANU, ESP_PAN_ROLE_NAP);
    if (status == ESP_OK) {
        phase = CONNECTING;
        retry_pending = false;
        ESP_LOGI(TAG, "MAPPIN_BT_CONNECTING");
    } else {
        ESP_LOGW(TAG, "PAN connection request failed: %s", esp_err_to_name(status));
        schedule_retry();
    }
}

static void connect_bonded(void)
{
    retry_pending = false;
    if (!reconnect_bonded || esp_bt_gap_get_bond_device_num() != 1)
        return;
    int count = 1;
    esp_bd_addr_t known;
    if (esp_bt_gap_get_bond_device_list(&count, &known) != ESP_OK || count != 1)
        return;
    memcpy(target, known, sizeof(target));
    connect_target();
}

static void command(const char *line)
{
    if (extra_command && extra_command(line))
        return;
    if (!strcmp(line, "scan") && phase == IDLE) {
        if (esp_bt_gap_start_discovery(ESP_BT_INQ_MODE_GENERAL_INQUIRY, 6, 0) == ESP_OK) {
            phase = SCANNING;
            ESP_LOGI(TAG, "MAPPIN_BT_SCANNING");
        }
        return;
    }
    if (!strncmp(line, "connect ", 8) && phase == IDLE && parse_address(line + 8, target)) {
        connect_target();
        return;
    }
    if (!strncmp(line, "confirm ", 8) && confirmation_pending) {
        char code[7];
        snprintf(code, sizeof(code), "%06" PRIu32, confirmation_code);
        if (!strcmp(line + 8, code)) {
            confirmation_pending = false;
            ESP_ERROR_CHECK_WITHOUT_ABORT(esp_bt_gap_ssp_confirm_reply(target, true));
            return;
        }
    }
    if (!strcmp(line, "check") && phase == CONNECTED) {
        xTaskNotifyGive(probe_task);
        return;
    }
    if (!strcmp(line, "disconnect") && phase == CONNECTED) {
        reconnect_bonded = false;
        retry_pending = false;
        ESP_ERROR_CHECK_WITHOUT_ABORT(esp_pan_disconnect(handle));
        return;
    }
    ESP_LOGI(TAG, "Commands: scan | connect AA:BB:CC:DD:EE:FF | confirm 123456 | check | disconnect");
}

static void handle_event(struct event *event)
{
    switch (event->type) {
    case PAN_READY:
        if (event->value != ESP_PAN_SUCCESS) {
            ESP_LOGE(TAG, "PAN registration failed (%" PRIu32 ")", event->value);
            return;
        }
        phase = IDLE;
        ESP_LOGI(TAG, "MAPPIN_BT_READY Classic PANU; Wi-Fi off; send scan");
        connect_bonded();
        break;
    case SCAN_DONE:
        if (phase == SCANNING)
            phase = IDLE;
        ESP_LOGI(TAG, "MAPPIN_BT_SCAN_DONE");
        break;
    case PAIR_CONFIRM:
        if (phase != CONNECTING || memcmp(event->address, target, sizeof(target))) {
            esp_bt_gap_ssp_confirm_reply(event->address, false);
            return;
        }
        confirmation_pending = true;
        confirmation_code = event->value;
        ESP_LOGI(TAG, "MAPPIN_BT_CONFIRM %06" PRIu32, confirmation_code);
        break;
    case PAN_OPEN:
        confirmation_pending = false;
        if (event->value != ESP_PAN_SUCCESS) {
            phase = IDLE;
            ESP_LOGW(TAG, "MAPPIN_BT_OPEN_FAILED status=%" PRIu32, event->value);
            schedule_retry();
            return;
        }
        if (phase != CONNECTING || memcmp(event->address, target, sizeof(target))) {
            esp_pan_disconnect(event->handle);
            return;
        }
        handle = event->handle;
        phase = CONNECTED;
        pan_link_up(handle);
        esp_netif_create_ip6_linklocal(pan);
        ESP_LOGI(TAG, "MAPPIN_BT_LINK_UP");
        break;
    case PAN_CLOSE:
        if (phase == CONNECTED && handle == event->handle) {
            pan_link_down(handle);
            handle = ESP_PAN_INVALID_HANDLE;
            phase = IDLE;
            ESP_LOGI(TAG, "MAPPIN_BT_LINK_DOWN");
            schedule_retry();
        }
        break;
    }
}

static void serial_console(void *ctx)
{
    char line[48];
    size_t length = 0;
    bool overflow = false;
    for (;;) {
        struct event event;
        while (xQueueReceive(events, &event, 0) == pdTRUE)
            handle_event(&event);
        if (phase == IDLE && retry_pending && (int32_t)(xTaskGetTickCount() - retry_at) >= 0)
            connect_bonded();
        uint8_t byte;
        if (uart_read_bytes(UART_NUM_0, &byte, 1, pdMS_TO_TICKS(25)) != 1)
            continue;
        if (byte == '\r' || byte == '\n') {
            if (!overflow && length) {
                line[length] = '\0';
                command(line);
            }
            length = 0;
            overflow = false;
        } else if (length + 1 < sizeof(line)) {
            line[length++] = byte;
        } else {
            overflow = true;
        }
    }
}

void mappin_pan_start(bool diagnostic, mappin_pan_command_fn command_hook)
{
    extra_command = command_hook;
    reconnect_bonded = !diagnostic;
    ESP_ERROR_CHECK(nvs_flash_init());
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    ESP_ERROR_CHECK(uart_driver_install(UART_NUM_0, 256, 0, 0, NULL, 0));

    events = xQueueCreate(12, sizeof(struct event));
    ESP_ERROR_CHECK(events ? ESP_OK : ESP_ERR_NO_MEM);
    ESP_ERROR_CHECK(esp_bt_controller_mem_release(ESP_BT_MODE_BLE));
    esp_bt_controller_config_t controller = BT_CONTROLLER_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_bt_controller_init(&controller));
    ESP_ERROR_CHECK(esp_bt_controller_enable(ESP_BT_MODE_CLASSIC_BT));
    esp_bluedroid_config_t host = BT_BLUEDROID_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_bluedroid_init_with_cfg(&host));
    ESP_ERROR_CHECK(esp_bluedroid_enable());
    ESP_ERROR_CHECK(esp_bt_gap_set_device_name("MapPin-Box BT"));
    ESP_ERROR_CHECK(esp_bt_gap_set_scan_mode(ESP_BT_CONNECTABLE, ESP_BT_GENERAL_DISCOVERABLE));
    esp_bt_io_cap_t capability = ESP_BT_IO_CAP_IO;
    ESP_ERROR_CHECK(esp_bt_gap_set_security_param(ESP_BT_SP_IOCAP_MODE, &capability, sizeof(uint8_t)));
    ESP_ERROR_CHECK(esp_bt_gap_register_callback(gap_event));

    pan = pan_link_create(esp_bt_dev_get_address());
    ESP_ERROR_CHECK(pan ? ESP_OK : ESP_ERR_NO_MEM);
    mdns_ready = diagnostic && mdns_init() == ESP_OK && mdns_register_netif(pan) == ESP_OK;
    ESP_ERROR_CHECK(xTaskCreate(check_peer, "pan-probe", 4096, NULL, 4, &probe_task) == pdPASS ?
                    ESP_OK : ESP_ERR_NO_MEM);
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_ETH_GOT_IP, got_ip, NULL));
    if (diagnostic) {
        httpd_handle_t server = NULL;
        httpd_config_t http = HTTPD_DEFAULT_CONFIG();
        ESP_ERROR_CHECK(httpd_start(&server, &http));
        const httpd_uri_t page = { .uri = "/", .method = HTTP_GET, .handler = home };
        ESP_ERROR_CHECK(httpd_register_uri_handler(server, &page));
    }

    ESP_ERROR_CHECK(esp_pan_register_callback(pan_event));
    esp_pan_cfg_t config = ESP_PAN_DEFAULT_CONFIG();
    ESP_ERROR_CHECK(esp_pan_init(&config));

    ESP_ERROR_CHECK(xTaskCreate(serial_console, "pan-console", 4096, NULL, 4, NULL) == pdPASS ?
                    ESP_OK : ESP_ERR_NO_MEM);
}

uint32_t mappin_pan_gateway(void)
{
    esp_netif_ip_info_t info;
    if (!pan || !esp_netif_is_netif_up(pan) || esp_netif_get_ip_info(pan, &info) != ESP_OK)
        return 0;
    return info.gw.addr;
}

uint32_t mappin_pan_address(void)
{
    esp_netif_ip_info_t info;
    if (!pan || !esp_netif_is_netif_up(pan) || esp_netif_get_ip_info(pan, &info) != ESP_OK)
        return 0;
    return info.ip.addr;
}

int mappin_pan_bond_count(void)
{
    return esp_bt_gap_get_bond_device_num();
}

esp_err_t mappin_pan_mdns_enable(void)
{
    esp_err_t err = mdns_register_netif(pan);
    if (err != ESP_OK)
        return err;
    pan_link_mdns_reset();
    return mdns_netif_action(pan, MDNS_EVENT_ENABLE_IP4);
}

void mappin_pan_mdns_report(void)
{
    pan_link_mdns_report();
}
