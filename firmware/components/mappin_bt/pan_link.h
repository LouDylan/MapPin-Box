#pragma once

#include "esp_netif.h"
#include "esp_pan_api.h"

esp_netif_t *pan_link_create(const uint8_t *mac);
void pan_link_up(uint16_t handle);
void pan_link_down(uint16_t handle);
void pan_link_congestion(uint16_t handle, bool congested);
void pan_link_receive(const esp_pan_cb_param_t *event);
void pan_link_mdns_reset(void);
void pan_link_mdns_report(void);
