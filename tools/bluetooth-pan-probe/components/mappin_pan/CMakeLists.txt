set(pan_link "${CMAKE_CURRENT_LIST_DIR}/../../../../firmware/components/mappin_bt")
idf_component_register(SRCS "mappin_pan.c" "${pan_link}/pan_link.c"
                       INCLUDE_DIRS "." "${pan_link}"
                       PRIV_REQUIRES bt esp_netif esp_event esp_http_server
                                     esp_driver_uart nvs_flash lwip)
target_compile_definitions(${COMPONENT_LIB} PRIVATE MAPPIN_PAN_TRACE)
