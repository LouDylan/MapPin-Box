#include <stdatomic.h>
#include <stdio.h>
#include <string.h>

#include "esp_event.h"
#include "esp_log.h"
#include "esp_netif.h"
#include "esp_wifi.h"
#include "mdns.h"
#include "setup_wifi.h"
#include "mappin_pan.h"

static const char *TAG = "mappin_setup";
static _Atomic(esp_netif_t *) ap;
static _Atomic(esp_netif_t *) station;
static atomic_int join_state;
enum { JOIN_IDLE, JOIN_PENDING, JOIN_READY, JOIN_FAILED };

static void wifi_event(void *ctx, esp_event_base_t base, int32_t id, void *data)
{
    if (id == WIFI_EVENT_AP_START)
        ESP_ERROR_CHECK_WITHOUT_ABORT(esp_netif_create_ip6_linklocal(ap));
    if (id == WIFI_EVENT_STA_CONNECTED)
        ESP_ERROR_CHECK_WITHOUT_ABORT(esp_netif_create_ip6_linklocal(station));
    if (id == WIFI_EVENT_STA_DISCONNECTED && atomic_load(&join_state) != JOIN_IDLE) {
        const wifi_event_sta_disconnected_t *event = data;
        atomic_store(&join_state, JOIN_FAILED);
        ESP_LOGW(TAG, "MAPPIN_SETUP_STA_DISCONNECTED reason=%d", event->reason);
    }
}

static void station_ip(void *ctx, esp_event_base_t base, int32_t id, void *data)
{
    const ip_event_got_ip_t *event = data;
    if (event->esp_netif != station)
        return;
    atomic_store(&join_state, JOIN_READY);
    ESP_LOGI(TAG, "MAPPIN_SETUP_STA_IP " IPSTR, IP2STR(&event->ip_info.ip));
}

static esp_err_t setup_wifi_init(void)
{
    esp_netif_t *ap_netif = esp_netif_create_default_wifi_ap();
    esp_netif_t *sta_netif = esp_netif_create_default_wifi_sta();
    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    init.nvs_enable = 0;
    esp_err_t err = esp_wifi_init(&init);
    if (err != ESP_OK)
        goto out_netifs;

    wifi_config_t config = { .ap = {
        .ssid = "MapPin-Pair-Test",
        .password = "mappin-setup",
        .channel = 6,
        .authmode = WIFI_AUTH_WPA2_PSK,
        .max_connection = 1,
    } };
    err = esp_wifi_set_storage(WIFI_STORAGE_RAM);
    if (err == ESP_OK)
        err = esp_wifi_set_mode(WIFI_MODE_AP);
    if (err == ESP_OK)
        err = esp_wifi_set_config(WIFI_IF_AP, &config);
    if (err != ESP_OK)
        goto out_wifi;

    err = esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, wifi_event, NULL);
    if (err != ESP_OK)
        goto out_wifi;
    err = esp_event_handler_register(IP_EVENT, IP_EVENT_STA_GOT_IP, station_ip, NULL);
    if (err != ESP_OK)
        goto out_event;

    station = sta_netif;
    ap = ap_netif;
    return ESP_OK;

out_event:
    esp_event_handler_unregister(WIFI_EVENT, ESP_EVENT_ANY_ID, wifi_event);
out_wifi:
    esp_wifi_deinit();
out_netifs:
    esp_netif_destroy_default_wifi(sta_netif);
    esp_netif_destroy_default_wifi(ap_netif);
    return err;
}

esp_err_t mappin_setup_start(void)
{
    esp_err_t err = ap ? ESP_OK : setup_wifi_init();
    if (err == ESP_OK)
        err = esp_wifi_start();
    ESP_LOGI(TAG, "MAPPIN_SETUP_WIFI %s", esp_err_to_name(err));
    return err;
}

void mappin_setup_stop(void)
{
    atomic_store(&join_state, JOIN_IDLE);
    if (ap && esp_netif_is_netif_up(ap))
        ESP_ERROR_CHECK_WITHOUT_ABORT(esp_wifi_stop());
}

bool mappin_setup_command(const char *command)
{
    if (!strcmp(command, "setup-wifi")) {
        mappin_setup_start();
        return true;
    }
    if (!strcmp(command, "setup-wifi-off")) {
        mappin_setup_stop();
        return true;
    }
    return false;
}

esp_err_t mappin_setup_join(const uint8_t *bytes, size_t len)
{
    if (!ap || !len || !esp_netif_is_netif_up(ap))
        return ESP_ERR_INVALID_STATE;
    size_t ssid_len = bytes[0];
    if (!ssid_len || ssid_len > 32 || ssid_len >= len || memchr(bytes + 1, 0, len - 1))
        return ESP_ERR_INVALID_ARG;
    size_t pass_len = len - 1 - ssid_len;
    if (pass_len && (pass_len < 8 || pass_len > 63))
        return ESP_ERR_INVALID_ARG;
    if (atomic_load(&join_state) == JOIN_PENDING || atomic_load(&join_state) == JOIN_READY)
        return ESP_ERR_INVALID_STATE;

    wifi_config_t config = { 0 };
    memcpy(config.sta.ssid, bytes + 1, ssid_len);
    memcpy(config.sta.password, bytes + 1 + ssid_len, pass_len);
    config.sta.threshold.authmode = pass_len ? WIFI_AUTH_WPA2_PSK : WIFI_AUTH_OPEN;
    esp_err_t err = esp_wifi_set_mode(WIFI_MODE_APSTA);
    if (err == ESP_OK)
        err = esp_wifi_set_config(WIFI_IF_STA, &config);
    if (err == ESP_OK) {
        atomic_store(&join_state, JOIN_PENDING);
        err = esp_wifi_connect();
    }
    if (err != ESP_OK)
        atomic_store(&join_state, JOIN_FAILED);
    return err;
}

static uint32_t address(esp_netif_t *netif)
{
    esp_netif_ip_info_t info;
    if (!netif || !esp_netif_is_netif_up(netif) || esp_netif_get_ip_info(netif, &info) != ESP_OK)
        return 0;
    return info.ip.addr;
}

uint32_t mappin_setup_address(void)
{
    return address(ap);
}

uint32_t mappin_setup_sta_address(void)
{
    return atomic_load(&join_state) == JOIN_READY ? address(station) : 0;
}

void mappin_setup_status(char *reply, size_t capacity)
{
    const char *status = "idle";
    esp_ip4_addr_t ip = { .addr = mappin_setup_sta_address() };
    if (ip.addr)
        status = "connected";
    else if (atomic_load(&join_state) == JOIN_PENDING)
        status = "joining";
    else if (atomic_load(&join_state) == JOIN_FAILED)
        status = "failed";
    snprintf(reply, capacity, "{\"state\":\"%s\",\"ip\":\"" IPSTR "\"}", status, IP2STR(&ip));
}

static esp_netif_t *wifi_interface(uint32_t address)
{
    if (!address)
        return NULL;
    if (address == mappin_setup_sta_address())
        return station;
    if (address == mappin_setup_address())
        return ap;
    return NULL;
}

int mappin_setup_ifindex(uint32_t address)
{
    esp_netif_t *netif = wifi_interface(address);
    return netif ? esp_netif_get_netif_impl_index(netif) : 0;
}

esp_err_t mappin_location_mdns_enable(uint32_t address)
{
    esp_netif_t *netif = wifi_interface(address);
    mdns_event_actions_t disable = MDNS_EVENT_DISABLE_IP4 | MDNS_EVENT_DISABLE_IP6;
    if (ap && netif != ap)
        mdns_netif_action(ap, disable);
    if (station && netif != station)
        mdns_netif_action(station, disable);
    if (netif)
        return mdns_netif_action(netif, MDNS_EVENT_ENABLE_IP4 | MDNS_EVENT_ENABLE_IP6);
    if (address && address == mappin_pan_address())
        return mappin_pan_mdns_enable();
    return ESP_ERR_INVALID_STATE;
}

void mappin_location_mdns_report(uint32_t address)
{
    if (!wifi_interface(address))
        mappin_pan_mdns_report();
}
