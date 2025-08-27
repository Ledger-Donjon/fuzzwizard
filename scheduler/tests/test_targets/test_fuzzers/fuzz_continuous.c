#include <stdint.h>
#include <stddef.h>

void overflow(uint8_t *buffer, size_t len);

int LLVMFuzzerTestOneInput(uint8_t *data, size_t size)
{
    return 0;
}