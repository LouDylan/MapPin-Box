#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

#include "freertos/FreeRTOS.h"
#include "esp_event.h"
#include "esp_log.h"
#include "esp_mac.h"
#include "esp_netif.h"
#include "esp_netif_defaults.h"
#include "tinyusb.h"
#include "tinyusb_default_config.h"
#include "tinyusb_net.h"
#include "usb_network.h"

#define FRAME_MAX 1518

static const char *TAG = "usb_network";
static esp_netif_t *usb_netif;
static _Atomic uint32_t rx_packets, tx_packets, rx_dropped, tx_dropped;
static _Atomic uint32_t attached, suspended, mounts, resumes;

static void free_rx(void *ctx, void *buffer)
{
    free(buffer);
}

static esp_err_t receive_frame(void *buffer, uint16_t len, void *ctx)
{
    if (len < 14 || len > FRAME_MAX) {
        atomic_fetch_add(&rx_dropped, 1);
        return ESP_ERR_INVALID_SIZE;
    }
    /* TinyUSB immediately reuses its RX buffer after this callback returns.
     * lwIP owns this copy until it calls free_rx, including on input failure. */
    void *copy = malloc(len);
    if (!copy) {
        atomic_fetch_add(&rx_dropped, 1);
        return ESP_ERR_NO_MEM;
    }
    memcpy(copy, buffer, len);
    esp_err_t err = esp_netif_receive(ctx, copy, len, NULL);
    atomic_fetch_add(err == ESP_OK ? &rx_packets : &rx_dropped, 1);
    return err;
}

static esp_err_t transmit_frame(void *ctx, void *buffer, size_t len)
{
    if (len > FRAME_MAX)
        return ESP_ERR_INVALID_SIZE;
    /* esp-netif calls from the serialized lwIP task. The bounded synchronous
     * send copies data before the caller releases its pbuf. Never call it from
     * TinyUSB callbacks, which would wait on their own task. */
    esp_err_t err = tinyusb_net_send_sync(buffer, len, NULL, pdMS_TO_TICKS(100));
    atomic_fetch_add(err == ESP_OK ? &tx_packets : &tx_dropped, 1);
    return err;
}

static void usb_event(tinyusb_event_t *event, void *ctx)
{
    switch (event->id) {
    case TINYUSB_EVENT_ATTACHED:
        atomic_store(&attached, 1);
        atomic_store(&suspended, 0);
        atomic_fetch_add(&mounts, 1);
        ESP_LOGI(TAG, "USB configured");
        break;
    case TINYUSB_EVENT_DETACHED:
        atomic_store(&attached, 0);
        atomic_store(&suspended, 0);
        ESP_LOGI(TAG, "USB detached");
        break;
    case TINYUSB_EVENT_SUSPENDED:
        atomic_store(&suspended, 1);
        ESP_LOGI(TAG, "USB suspended");
        break;
    case TINYUSB_EVENT_RESUMED:
        atomic_store(&suspended, 0);
        atomic_fetch_add(&resumes, 1);
        ESP_LOGI(TAG, "USB resumed");
        break;
    }
}

static void local_dhcp(esp_netif_dhcp_option_mode_t mode)
{
    const esp_netif_dhcp_option_id_t options[] = {
        ESP_NETIF_ROUTER_SOLICITATION_ADDRESS,
        ESP_NETIF_DOMAIN_NAME_SERVER,
    };
    for (size_t i = 0; i < sizeof(options) / sizeof(options[0]); i++) {
        uint8_t offer = 0;
        ESP_ERROR_CHECK(esp_netif_dhcps_option(usb_netif, mode,
                                             options[i], &offer, sizeof(offer)));
        ESP_ERROR_CHECK(offer == 0 ? ESP_OK : ESP_FAIL);
    }
}

static void start_netif(uint8_t host_mac[6])
{
    esp_netif_ip_info_t ip = {0};
    ESP_ERROR_CHECK(esp_netif_str_to_ip4(WAYPIN_USB_ADDRESS, &ip.ip));
    ESP_ERROR_CHECK(esp_netif_str_to_ip4("255.255.255.0", &ip.netmask));
    esp_netif_inherent_config_t base = ESP_NETIF_INHERENT_DEFAULT_ETH();
    base.flags = ESP_NETIF_DHCP_SERVER | ESP_NETIF_FLAG_AUTOUP;
    base.ip_info = &ip;
    base.if_key = "ETH_DEF";
    base.if_desc = "usb-ncm";
    base.get_ip_event = 0;
    base.lost_ip_event = 0;
    base.route_prio = 10;
    esp_netif_config_t config = {
        .base = &base,
        .stack = ESP_NETIF_NETSTACK_DEFAULT_ETH,
    };
    usb_netif = esp_netif_new(&config);
    ESP_ERROR_CHECK(usb_netif ? ESP_OK : ESP_ERR_NO_MEM);
    const esp_netif_driver_ifconfig_t driver = {
        .handle = usb_netif,
        .transmit = transmit_frame,
        .driver_free_rx_buffer = free_rx,
    };
    ESP_ERROR_CHECK(esp_netif_set_driver_config(usb_netif, &driver));

    uint8_t device_mac[6];
    ESP_ERROR_CHECK(esp_read_mac(device_mac, ESP_MAC_ETH));
    memcpy(host_mac, device_mac, sizeof(device_mac));
    host_mac[0] = (host_mac[0] | 2) & 0xfe;
    host_mac[5] ^= 1;
    /* The descriptor identifies the HOST end. lwIP must use a different MAC. */
    ESP_ERROR_CHECK(esp_netif_set_mac(usb_netif, device_mac));
    ESP_ERROR_CHECK(esp_netif_set_hostname(usb_netif, "waypin-usb"));
    local_dhcp(ESP_NETIF_OP_SET);
    /* A permanently available local interface keeps DHCP ready before the host
     * configures NCM. Physical USB state is reported separately. Suspend keeps
     * the lease; no network operation runs in the TinyUSB callback. */
    esp_netif_action_start(usb_netif, NULL, 0, NULL);
    esp_netif_dhcp_status_t status;
    ESP_ERROR_CHECK(esp_netif_dhcps_get_status(usb_netif, &status));
    ESP_ERROR_CHECK(status == ESP_NETIF_DHCP_STARTED ? ESP_OK : ESP_FAIL);
    local_dhcp(ESP_NETIF_OP_GET);
}

void waypin_usb_network_start(void)
{
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    tinyusb_net_config_t network = {.on_recv_callback = receive_frame};
    start_netif(network.mac_addr);
    network.user_context = usb_netif;
    tinyusb_config_t usb = TINYUSB_DEFAULT_CONFIG(usb_event, NULL);
    ESP_ERROR_CHECK(tinyusb_driver_install(&usb));
    ESP_ERROR_CHECK(tinyusb_net_init(&network));
}

void waypin_usb_network_status(struct waypin_usb_network_status *status)
{
    *status = (struct waypin_usb_network_status) {
        .attached = atomic_load(&attached),
        .suspended = atomic_load(&suspended),
        .mounts = atomic_load(&mounts),
        .resumes = atomic_load(&resumes),
        .rx_packets = atomic_load(&rx_packets),
        .tx_packets = atomic_load(&tx_packets),
        .rx_dropped = atomic_load(&rx_dropped),
        .tx_dropped = atomic_load(&tx_dropped),
    };
}
