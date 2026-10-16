/* Owned, serialized access to the embedded local mailbox. */
#include <stdint.h>

typedef struct MobileEngine MobileEngine;
MobileEngine *mermoz_mobile_open(const char *home, char **error);
char *mermoz_mobile_request(MobileEngine *engine, const char *request);
void mermoz_mobile_close(MobileEngine *engine);
void mermoz_mobile_string_free(char *value);
