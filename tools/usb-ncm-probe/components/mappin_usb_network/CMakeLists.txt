idf_component_register(SRCS "usb_network.c"
                       INCLUDE_DIRS "."
                       PRIV_REQUIRES esp_netif esp_event esp_hw_support)
