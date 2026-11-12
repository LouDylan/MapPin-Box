if(IDF_TARGET STREQUAL "esp32")
    idf_component_register(SRCS "bluetooth.c" "pan_link.c"
                           PRIV_REQUIRES bt esp_netif esp_event nvs_flash espressif__mdns)
else()
    idf_component_register()
endif()
