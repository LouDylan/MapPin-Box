#include <inttypes.h>
#include <stdlib.h>
#include <string.h>

#include "esp_bt.h"
#include "esp_bt_device.h"
#include "esp_bt_main.h"
#include "esp_check.h"
#include "esp_event.h"
#include "esp_gap_bt_api.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"
#include "mdns.h"
#include "nvs.h"
#include "pan_link.h"

#if CONFIG_MDNS_MAX_INTERFACES <= (defined(CONFIG_MDNS_PREDEF_NETIF_STA) + \
                                  defined(CONFIG_MDNS_PREDEF_NETIF_AP) + \
                                  defined(CONFIG_MDNS_PREDEF_NETIF_ETH))
#error "Bluetooth PAN needs a free mDNS interface slot"
#endif

/* The pinned Bluedroid SDK has no public classic GAP call for bonding without PAN. */
extern void BTA_DmBond(esp_bd_addr_t address);
extern void BTA_DmBondCancel(esp_bd_addr_t address);

enum phase { STARTING, IDLE, SCANNING, CONNECTING, CONFIRMING, LINK, FAILED };
enum action { SCAN, CONNECT, CONFIRM, READY, SCAN_DONE, COMPARE, AUTH, OPEN, CLOSE, BUTTON_SCAN };

#define MAX_PEERS 8
#define NAME_SIZE 65
#define RETRY_MS 10000
#define CONNECT_MS 45000

struct event {
    enum action action;
    esp_bd_addr_t address;
    uint32_t value;
    uint16_t handle;
};

struct peer {
    esp_bd_addr_t address;
    char name[NAME_SIZE];
};

static const char *TAG = "mappin_bt";
static QueueHandle_t events;
static esp_netif_t *pan;
static portMUX_TYPE view_lock = portMUX_INITIALIZER_UNLOCKED;
static struct peer peers[MAX_PEERS];
static unsigned peer_count;
static enum phase phase;
static uint32_t pin;
static bool button_pairing;

/* The connection task owns the target and reconnect policy. */
static esp_bd_addr_t target;
static esp_bd_addr_t preferred;
static bool have_preferred;
static bool bonding;
static uint16_t handle = ESP_PAN_INVALID_HANDLE;
static TickType_t retry_at, connect_until;

static void publish(enum phase next, uint32_t code)
{
    portENTER_CRITICAL(&view_lock);
    phase = next;
    pin = code;
    if (button_pairing && (next == IDLE || next == LINK || next == FAILED)) {
        button_pairing = false;
        peer_count = 0;
    }
    portEXIT_CRITICAL(&view_lock);
}

static void post(struct event event)
{
    if (xQueueSend(events, &event, 0) != pdTRUE)
        ESP_LOGW(TAG, "Bluetooth event queue full");
}

static void found(const esp_bt_gap_cb_param_t *data)
{
    const uint8_t *name = NULL;
    uint8_t len = 0;
    uint32_t cod = 0;
    for (int i = 0; i < data->disc_res.num_prop; i++) {
        const esp_bt_gap_dev_prop_t *prop = &data->disc_res.prop[i];
        if (prop->type == ESP_BT_GAP_DEV_PROP_COD && prop->len == (int)sizeof(cod)) {
            memcpy(&cod, prop->val, sizeof(cod));
        } else if (prop->type == ESP_BT_GAP_DEV_PROP_BDNAME && prop->len > 0) {
            name = prop->val;
            len = prop->len < NAME_SIZE ? prop->len : NAME_SIZE - 1;
        } else if (prop->type == ESP_BT_GAP_DEV_PROP_EIR && !name) {
            name = esp_bt_gap_resolve_eir_data(prop->val, ESP_BT_EIR_TYPE_CMPL_LOCAL_NAME, &len);
            if (!name)
                name = esp_bt_gap_resolve_eir_data(prop->val, ESP_BT_EIR_TYPE_SHORT_LOCAL_NAME, &len);
        }
    }
    if (len >= NAME_SIZE)
        len = NAME_SIZE - 1;
    bool phone = esp_bt_gap_get_cod_major_dev(cod) == ESP_BT_COD_MAJOR_DEV_PHONE;

    portENTER_CRITICAL(&view_lock);
    if (phase == SCANNING && (button_pairing ? phone : name && len)) {
        unsigned i = 0;
        while (i < peer_count && memcmp(peers[i].address, data->disc_res.bda, ESP_BD_ADDR_LEN))
            i++;
        if (i < MAX_PEERS) {
            memcpy(peers[i].address, data->disc_res.bda, ESP_BD_ADDR_LEN);
            if (name && len)
                memcpy(peers[i].name, name, len);
            else
                len = 0;
            peers[i].name[len] = '\0';
            if (i == peer_count)
                peer_count++;
        }
    }
    portEXIT_CRITICAL(&view_lock);
}

static void gap_event(esp_bt_gap_cb_event_t type, esp_bt_gap_cb_param_t *data)
{
    switch (type) {
    case ESP_BT_GAP_DISC_RES_EVT:
        found(data);
        break;
    case ESP_BT_GAP_DISC_STATE_CHANGED_EVT:
        if (data->disc_st_chg.state == ESP_BT_GAP_DISCOVERY_STOPPED)
            post((struct event){ .action = SCAN_DONE });
        break;
    case ESP_BT_GAP_CFM_REQ_EVT: {
        struct event event = { .action = COMPARE, .value = data->cfm_req.num_val };
        memcpy(event.address, data->cfm_req.bda, ESP_BD_ADDR_LEN);
        post(event);
        break;
    }
    case ESP_BT_GAP_AUTH_CMPL_EVT: {
        struct event event = { .action = AUTH, .value = data->auth_cmpl.stat };
        memcpy(event.address, data->auth_cmpl.bda, ESP_BD_ADDR_LEN);
        post(event);
        break;
    }
    case ESP_BT_GAP_PIN_REQ_EVT:
        esp_bt_gap_pin_reply(data->pin_req.bda, false, 0, NULL);
        break;
    default:
        break;
    }
}

static void pan_event(esp_pan_cb_event_t type, esp_pan_cb_param_t *data)
{
    switch (type) {
    case ESP_PAN_SET_ROLE_EVT:
        post((struct event){ .action = READY, .value = data->set_role.status });
        break;
    case ESP_PAN_OPEN_EVT: {
        struct event event = {
            .action = OPEN, .handle = data->open.handle, .value = data->open.status,
        };
        memcpy(event.address, data->open.remote_bda, ESP_BD_ADDR_LEN);
        post(event);
        break;
    }
    case ESP_PAN_CLOSE_EVT:
        post((struct event){ .action = CLOSE, .handle = data->close.handle });
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

static void load_preferred(void)
{
    const char *names[] = { "mappin-bt", "waypin-bt" };
    for (size_t i = 0; i < sizeof(names) / sizeof(*names); i++) {
        nvs_handle_t nvs;
        if (nvs_open(names[i], NVS_READONLY, &nvs) != ESP_OK)
            continue;
        size_t size = sizeof(preferred);
        have_preferred = nvs_get_blob(nvs, "peer_v1", preferred, &size) == ESP_OK &&
                         size == sizeof(preferred);
        nvs_close(nvs);
        if (have_preferred)
            break;
    }
}

static void save_preferred(void)
{
    if (have_preferred && !memcmp(target, preferred, sizeof(preferred)))
        return;
    nvs_handle_t nvs;
    esp_err_t err = nvs_open("mappin-bt", NVS_READWRITE, &nvs);
    if (err == ESP_OK) {
        err = nvs_set_blob(nvs, "peer_v1", target, sizeof(target));
        if (err == ESP_OK)
            err = nvs_commit(nvs);
        nvs_close(nvs);
    }
    if (err != ESP_OK)
        ESP_LOGW(TAG, "Could not save Bluetooth peer: %s", esp_err_to_name(err));
    memcpy(preferred, target, sizeof(preferred));
    have_preferred = true;
}

static void retry_after(unsigned ms)
{
    retry_at = xTaskGetTickCount() + pdMS_TO_TICKS(ms);
}

static void failed(void)
{
    retry_after(bonding ? 30000 : RETRY_MS);
    bonding = false;
    publish(FAILED, 0);
    ESP_LOGW(TAG, "MAPPIN_BT_LINK_FAILED");
}

static void connect(void)
{
    ESP_LOGI(TAG, "Bluetooth connect internal RAM: %u free, %u largest",
             (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT),
             (unsigned)heap_caps_get_largest_free_block(MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT));
    if (esp_pan_connect(target, ESP_PAN_ROLE_PANU, ESP_PAN_ROLE_NAP) != ESP_OK) {
        failed();
        return;
    }
    publish(CONNECTING, 0);
    connect_until = xTaskGetTickCount() + pdMS_TO_TICKS(CONNECT_MS);
    ESP_LOGI(TAG, "MAPPIN_BT_CONNECTING");
}

static void bond(void)
{
    bonding = true;
    publish(CONNECTING, 0);
    connect_until = xTaskGetTickCount() + pdMS_TO_TICKS(CONNECT_MS);
    BTA_DmBond(target);
    ESP_LOGI(TAG, "MAPPIN_BT_BONDING");
}

static void on_auth(const struct event *event)
{
    if (!bonding || memcmp(target, event->address, sizeof(target)))
        return;
    if (event->value != ESP_BT_STATUS_SUCCESS) {
        ESP_LOGW(TAG, "MAPPIN_BT_BOND_FAILED status=%" PRIu32, event->value);
        failed();
        return;
    }
    bonding = false;
    save_preferred();
    publish(IDLE, 0);
    retry_after(1000);
    ESP_LOGI(TAG, "MAPPIN_BT_BONDED");
}

static void confirm_bond(void)
{
    esp_err_t err = esp_bt_gap_ssp_confirm_reply(target, true);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "MAPPIN_BT_CONFIRM_FAILED %s", esp_err_to_name(err));
        BTA_DmBondCancel(target);
        failed();
        return;
    }
    publish(CONNECTING, 0);
    ESP_LOGI(TAG, "MAPPIN_BT_CONFIRM_SENT");
}

static void reconnect(void)
{
    retry_after(RETRY_MS);
    int count = esp_bt_gap_get_bond_device_num();
    if (count <= 0)
        return;
    esp_bd_addr_t *list = calloc(count, sizeof(*list));
    if (!list)
        return;
    bool known = false;
    if (esp_bt_gap_get_bond_device_list(&count, list) == ESP_OK) {
        for (int i = 0; i < count; i++) {
            if (count != 1 && (!have_preferred || memcmp(preferred, list[i], sizeof(preferred))))
                continue;
            memcpy(target, list[i], sizeof(target));
            known = true;
            break;
        }
    }
    free(list);
    if (known)
        connect();
}

static void scan(bool button)
{
    if (phase != IDLE && phase != FAILED)
        return;
    portENTER_CRITICAL(&view_lock);
    peer_count = 0;
    button_pairing = button;
    portEXIT_CRITICAL(&view_lock);
    publish(SCANNING, 0);
    if (esp_bt_gap_start_discovery(ESP_BT_INQ_MODE_GENERAL_INQUIRY, 6, 0) != ESP_OK)
        failed();
    else if (button)
        ESP_LOGI(TAG, "MAPPIN_BT_BUTTON_SCAN");
}

static void pair_scanned_phone(void)
{
    portENTER_CRITICAL(&view_lock);
    unsigned count = peer_count;
    int selected = count == 1 ? 0 : -1;
    for (unsigned i = 0; have_preferred && i < count; i++) {
        if (!memcmp(peers[i].address, preferred, sizeof(preferred))) {
            selected = i;
            break;
        }
    }
    if (selected >= 0)
        memcpy(target, peers[selected].address, sizeof(target));
    portEXIT_CRITICAL(&view_lock);

    if (selected >= 0)
        bond();
    else {
        ESP_LOGW(TAG, "MAPPIN_BT_BUTTON_SCAN_FOUND %u phones", count);
        failed();
    }
}

static void on_open(const struct event *event)
{
    bool expected = !bonding && phase == CONNECTING &&
                    !memcmp(event->address, target, sizeof(target));
    if (!expected) {
        if (event->value == ESP_PAN_SUCCESS)
            esp_pan_disconnect(event->handle);
        return;
    }
    if (event->value != ESP_PAN_SUCCESS) {
        failed();
        return;
    }
    handle = event->handle;
    pan_link_up(handle);
    publish(LINK, 0);
    save_preferred();
    ESP_LOGI(TAG, "MAPPIN_BT_LINK_UP");
}

static void on_event(const struct event *event)
{
    switch (event->action) {
    case READY:
        if (event->value != ESP_PAN_SUCCESS) {
            ESP_LOGE(TAG, "Bluetooth PAN registration failed");
            return;
        }
        publish(IDLE, 0);
        ESP_LOGI(TAG, "MAPPIN_BT_READY");
        reconnect();
        break;
    case SCAN:
        scan(false);
        break;
    case BUTTON_SCAN:
        scan(true);
        break;
    case SCAN_DONE:
        if (phase == SCANNING && button_pairing)
            pair_scanned_phone();
        else if (phase == SCANNING) {
            publish(IDLE, 0);
            retry_after(90000);
        }
        break;
    case CONNECT:
        if (phase == IDLE || phase == FAILED) {
            memcpy(target, event->address, sizeof(target));
            bond();
        }
        break;
    case COMPARE:
        if (!bonding || phase != CONNECTING || memcmp(target, event->address, sizeof(target))) {
            esp_bt_gap_ssp_confirm_reply((uint8_t *)event->address, false);
        } else if (button_pairing) {
            confirm_bond();
        } else {
            publish(CONFIRMING, event->value);
        }
        break;
    case CONFIRM:
        if (phase == CONFIRMING && pin == event->value)
            confirm_bond();
        break;
    case AUTH:
        on_auth(event);
        break;
    case OPEN:
        on_open(event);
        break;
    case CLOSE:
        if (phase == LINK && handle == event->handle) {
            mdns_netif_action(pan, MDNS_EVENT_DISABLE_IP4);
            pan_link_down(handle);
            handle = ESP_PAN_INVALID_HANDLE;
            publish(IDLE, 0);
            retry_after(RETRY_MS);
            ESP_LOGI(TAG, "MAPPIN_BT_LINK_DOWN");
        }
        break;
    }
}

static void connection_task(void *ctx)
{
    load_preferred();
    for (;;) {
        struct event event;
        if (xQueueReceive(events, &event, pdMS_TO_TICKS(250)) == pdTRUE)
            on_event(&event);
        TickType_t now = xTaskGetTickCount();
        if ((phase == IDLE || phase == FAILED) && (int32_t)(now - retry_at) >= 0)
            reconnect();
        if ((phase == CONNECTING || phase == CONFIRMING) && (int32_t)(now - connect_until) >= 0) {
            if (phase == CONFIRMING)
                esp_bt_gap_ssp_confirm_reply(target, false);
            if (bonding)
                BTA_DmBondCancel(target);
            failed();
        }
    }
}

static void got_ip(void *ctx, esp_event_base_t base, int32_t id, void *data)
{
    const ip_event_got_ip_t *event = data;
    if (event->esp_netif != pan || !esp_netif_is_netif_up(pan))
        return;
    ESP_LOGI(TAG, "MAPPIN_BT_IP http://" IPSTR "/", IP2STR(&event->ip_info.ip));

    esp_err_t err = mdns_register_netif(pan);
    /* A renewed lease can arrive while the interface is still registered. */
    if (err == ESP_OK || err == ESP_ERR_INVALID_STATE)
        err = mdns_netif_action(pan, MDNS_EVENT_ENABLE_IP4);
    if (err != ESP_OK)
        ESP_LOGW(TAG, "Bluetooth mDNS unavailable: %s", esp_err_to_name(err));
}

esp_err_t mappin_bt_start(void)
{
    events = xQueueCreate(12, sizeof(struct event));
    if (!events)
        return ESP_ERR_NO_MEM;
    ESP_RETURN_ON_ERROR(esp_bt_controller_mem_release(ESP_BT_MODE_BLE), TAG, "release BLE");
    esp_bt_controller_config_t controller = BT_CONTROLLER_INIT_CONFIG_DEFAULT();
    ESP_RETURN_ON_ERROR(esp_bt_controller_init(&controller), TAG, "init controller");
    ESP_RETURN_ON_ERROR(esp_bt_controller_enable(ESP_BT_MODE_CLASSIC_BT), TAG, "enable controller");
    esp_bluedroid_config_t host = BT_BLUEDROID_INIT_CONFIG_DEFAULT();
    ESP_RETURN_ON_ERROR(esp_bluedroid_init_with_cfg(&host), TAG, "init Bluedroid");
    ESP_RETURN_ON_ERROR(esp_bluedroid_enable(), TAG, "enable Bluedroid");
    ESP_RETURN_ON_ERROR(esp_bt_gap_set_device_name("MapPin-Box BT"), TAG, "set name");
    ESP_RETURN_ON_ERROR(esp_bt_gap_set_scan_mode(ESP_BT_CONNECTABLE, ESP_BT_NON_DISCOVERABLE),
                        TAG, "set scan mode");
    uint8_t capability = ESP_BT_IO_CAP_IO;
    ESP_RETURN_ON_ERROR(esp_bt_gap_set_security_param(ESP_BT_SP_IOCAP_MODE, &capability, 1),
                        TAG, "set numeric confirmation");
    ESP_RETURN_ON_ERROR(esp_bt_gap_register_callback(gap_event), TAG, "register GAP callback");
    pan = pan_link_create(esp_bt_dev_get_address());
    if (!pan)
        return ESP_ERR_NO_MEM;
    ESP_RETURN_ON_ERROR(esp_event_handler_register(IP_EVENT, IP_EVENT_ETH_GOT_IP, got_ip, NULL),
                        TAG, "register PAN IP handler");
    ESP_RETURN_ON_ERROR(esp_pan_register_callback(pan_event), TAG, "register PAN callback");
    esp_pan_cfg_t config = ESP_PAN_DEFAULT_CONFIG();
    ESP_RETURN_ON_ERROR(esp_pan_init(&config), TAG, "init PAN");
    return xTaskCreate(connection_task, "mappin-bt", 4096, NULL, 4, NULL) == pdPASS
        ? ESP_OK : ESP_ERR_NO_MEM;
}

bool mappin_bt_endpoint(uint32_t *local, uint32_t *peer)
{
    esp_netif_ip_info_t ip;
    if (!pan || !esp_netif_is_netif_up(pan) ||
        esp_netif_get_ip_info(pan, &ip) != ESP_OK || !ip.ip.addr || !ip.gw.addr)
        return false;
    *local = ip.ip.addr;
    *peer = ip.gw.addr;
    return true;
}

uint32_t mappin_bt_state(uint32_t *code, bool *button)
{
    portENTER_CRITICAL(&view_lock);
    uint32_t current = phase;
    *code = pin;
    *button = button_pairing;
    portEXIT_CRITICAL(&view_lock);
    return current;
}

bool mappin_bt_candidate(uint32_t index, char *name)
{
    portENTER_CRITICAL(&view_lock);
    bool valid = index < peer_count;
    if (valid)
        memcpy(name, peers[index].name, NAME_SIZE);
    portEXIT_CRITICAL(&view_lock);
    return valid;
}

esp_err_t mappin_bt_action(uint32_t action, uint32_t value)
{
    if (!events || action > CONFIRM)
        return ESP_ERR_INVALID_ARG;
    struct event event = { .action = action, .value = value };
    portENTER_CRITICAL(&view_lock);
    bool valid = phase == IDLE || phase == FAILED;
    if (action == CONNECT) {
        valid = valid && value < peer_count;
        if (valid)
            memcpy(event.address, peers[value].address, ESP_BD_ADDR_LEN);
    } else if (action == CONFIRM) {
        valid = phase == CONFIRMING && !button_pairing && pin == value;
    }
    portEXIT_CRITICAL(&view_lock);
    if (!valid)
        return ESP_ERR_INVALID_STATE;
    return xQueueSend(events, &event, 0) == pdTRUE ? ESP_OK : ESP_ERR_TIMEOUT;
}

bool mappin_bt_button(void)
{
    if (!events)
        return false;
    struct event event = { .action = BUTTON_SCAN };
    portENTER_CRITICAL(&view_lock);
    bool valid = phase == IDLE || phase == FAILED;
    portEXIT_CRITICAL(&view_lock);
    return valid && xQueueSend(events, &event, 0) == pdTRUE;
}
