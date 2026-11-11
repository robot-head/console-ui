#ifndef CONSOLE_UI_BRIDGE_H
#define CONSOLE_UI_BRIDGE_H
#include <stdint.h>
typedef struct Bridge Bridge;
uint32_t console_ui_abi_version(void);
Bridge *console_ui_create(void);
Bridge *console_ui_create_with_settings(const char *settings_json, const char *preferences_path);
char *console_ui_dispatch(Bridge *bridge, const char *action_json);
char *console_ui_view(Bridge *bridge);
const char *console_ui_last_error(void);
void console_ui_string_free(char *value);
void console_ui_destroy(Bridge *bridge);
#endif
