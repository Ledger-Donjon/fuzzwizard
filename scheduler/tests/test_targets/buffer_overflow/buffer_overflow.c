#include <stdint.h>
#include <stddef.h>
#include <string.h>

void overflow(uint8_t *buffer, size_t len)
{
    uint8_t buf[32];

    memcpy(buf, buffer, len);
}