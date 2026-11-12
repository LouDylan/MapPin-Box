#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "esp_err.h"

typedef bool (*mappin_pan_command_fn)(const char *);
void mappin_pan_start(bool diagnostic, mappin_pan_command_fn extra_command);
uint32_t mappin_pan_gateway(void);
uint32_t mappin_pan_address(void);
int mappin_pan_bond_count(void);
esp_err_t mappin_pan_mdns_enable(void);
void mappin_pan_mdns_report(void);
