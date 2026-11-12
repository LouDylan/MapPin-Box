/*
 * Ethernet framing and netif lifecycle adapted from ESP-IDF bt_pan_panu.
 * SPDX-FileCopyrightText: 2026 Espressif Systems (Shanghai) CO LTD
 * SPDX-License-Identifier: Unlicense OR CC0-1.0
 */

#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

#include "esp_log.h"
#include "pan_link.h"

#define ETH_HEADER_LEN 14
#define ETH_MAX_PAYLOAD 1500

static esp_netif_t *netif;
static atomic_uint active_handle = ESP_PAN_INVALID_HANDLE;
static atomic_bool congested;
#ifdef MAPPIN_PAN_TRACE
static atomic_uint mdns_frames[2][2]; /* IPv4/IPv6, RX/TX */

/* Count only Bonjour packets about the pairing service. No packet data is logged. */
static void count_pairing_mdns(uint16_t protocol, const uint8_t *ip, size_t len, bool tx)
{
    static const char label[] = "_remotepairing-pairable-host";
    size_t udp;
    unsigned family;
    switch (protocol) {
    case 0x0800:
        if (len < 20 || ip[9] != 17)
            return;
        udp = (ip[0] & 0x0f) * 4;
        family = 0;
        break;
    case 0x86dd:
        if (len < 40 || ip[6] != 17)
            return;
        udp = 40;
        family = 1;
        break;
    default:
        return;
    }
    if (len < udp + 8 + 12 ||
        !((ip[udp] == 0x14 && ip[udp + 1] == 0xe9) ||
          (ip[udp + 2] == 0x14 && ip[udp + 3] == 0xe9)))
        return;
    for (size_t i = udp + 8 + 12; i + sizeof(label) <= len; i++) {
        if (ip[i] != sizeof(label) - 1 || memcmp(ip + i + 1, label, sizeof(label) - 1))
            continue;
        atomic_fetch_add(&mdns_frames[family][tx], 1);
        return;
    }
}

void pan_link_mdns_reset(void)
{
    for (unsigned i = 0; i < 2; i++) {
        atomic_store(&mdns_frames[i][0], 0);
        atomic_store(&mdns_frames[i][1], 0);
    }
}

void pan_link_mdns_report(void)
{
    ESP_LOGI("pan_link", "MAPPIN_PAN_MDNS rx4=%u tx4=%u rx6=%u tx6=%u",
             atomic_load(&mdns_frames[0][0]), atomic_load(&mdns_frames[0][1]),
             atomic_load(&mdns_frames[1][0]), atomic_load(&mdns_frames[1][1]));
}
#endif

static esp_err_t transmit(void *ctx, void *buffer, size_t size)
{
    unsigned handle = atomic_load(&active_handle);
    if (handle == ESP_PAN_INVALID_HANDLE || atomic_load(&congested))
        return ESP_ERR_INVALID_STATE;
    if (size <= ETH_HEADER_LEN || size > ETH_HEADER_LEN + ETH_MAX_PAYLOAD)
        return ESP_ERR_INVALID_SIZE;

    uint8_t *frame = buffer;
    uint16_t protocol = ((uint16_t)frame[12] << 8) | frame[13];
    esp_err_t err = esp_pan_write(handle, frame, frame + ESP_PAN_MAC_ADDR_LEN, protocol,
                                 size - ETH_HEADER_LEN, frame + ETH_HEADER_LEN, false);
#ifdef MAPPIN_PAN_TRACE
    if (err == ESP_OK)
        count_pairing_mdns(protocol, frame + ETH_HEADER_LEN, size - ETH_HEADER_LEN, true);
#endif
    return err;
}

static void free_frame(void *ctx, void *buffer)
{
    free(buffer);
}

esp_netif_t *pan_link_create(const uint8_t *mac)
{
    esp_netif_inherent_config_t base = ESP_NETIF_INHERENT_DEFAULT_ETH();
    base.if_key = "MAPPIN_PAN";
    base.if_desc = "pan";
    const esp_netif_config_t config = {
        .base = &base,
        .stack = ESP_NETIF_NETSTACK_DEFAULT_ETH,
    };
    netif = esp_netif_new(&config);
    if (!netif)
        return NULL;
    const esp_netif_driver_ifconfig_t driver = {
        .handle = netif,
        .transmit = transmit,
        .driver_free_rx_buffer = free_frame,
    };
    esp_pan_mac_addr_t ethernet_mac;
    memcpy(ethernet_mac, mac, sizeof(ethernet_mac));
    if (esp_netif_set_driver_config(netif, &driver) != ESP_OK ||
        esp_netif_set_mac(netif, ethernet_mac) != ESP_OK) {
        esp_netif_destroy(netif);
        netif = NULL;
    }
    return netif;
}

void pan_link_up(uint16_t handle)
{
    atomic_store(&congested, false);
    atomic_store(&active_handle, handle);
    esp_netif_action_start(netif, NULL, 0, NULL);
    esp_netif_action_connected(netif, NULL, 0, NULL);
    esp_netif_create_ip6_linklocal(netif);
}

void pan_link_down(uint16_t handle)
{
    if (atomic_load(&active_handle) != handle)
        return;
    atomic_store(&active_handle, ESP_PAN_INVALID_HANDLE);
    esp_netif_action_disconnected(netif, NULL, 0, NULL);
    esp_netif_action_stop(netif, NULL, 0, NULL);
}

void pan_link_congestion(uint16_t handle, bool value)
{
    if (atomic_load(&active_handle) == handle)
        atomic_store(&congested, value);
}

void pan_link_receive(const esp_pan_cb_param_t *event)
{
    const struct pan_data_ind_evt_param *data = &event->data_ind;
    if (atomic_load(&active_handle) != data->handle || data->ext ||
        !data->len || data->len > ETH_MAX_PAYLOAD)
        return;
#ifdef MAPPIN_PAN_TRACE
    count_pairing_mdns(data->protocol, data->data, data->len, false);
#endif
    uint8_t *frame = malloc(ETH_HEADER_LEN + data->len);
    if (!frame)
        return;

    memcpy(frame, data->dst, ESP_PAN_MAC_ADDR_LEN);
    memcpy(frame + ESP_PAN_MAC_ADDR_LEN, data->src, ESP_PAN_MAC_ADDR_LEN);
    frame[12] = data->protocol >> 8;
    frame[13] = data->protocol & 0xff;
    memcpy(frame + ETH_HEADER_LEN, data->data, data->len);
    /* esp_netif_receive owns the frame and calls free_frame. */
    esp_netif_receive(netif, frame, ETH_HEADER_LEN + data->len, NULL);
}
