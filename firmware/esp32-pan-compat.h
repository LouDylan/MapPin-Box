#pragma once

/* esp-idf-sys still reads the legacy PCNT count on this ESP-IDF revision. */
#define SOC_PCNT_UNITS_PER_GROUP 8

#if __has_include("esp_crt_bundle.h")
#include "esp_crt_bundle.h"
#endif
