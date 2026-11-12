#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"

esp_err_t mappin_setup_start(void);
void mappin_setup_stop(void);
bool mappin_setup_command(const char *command);
esp_err_t mappin_setup_join(const uint8_t *bytes, size_t len);
void mappin_setup_status(char *reply, size_t capacity);
uint32_t mappin_setup_address(void);
uint32_t mappin_setup_sta_address(void);
int mappin_setup_ifindex(uint32_t address);
esp_err_t mappin_location_mdns_enable(uint32_t address);
void mappin_location_mdns_report(uint32_t address);
