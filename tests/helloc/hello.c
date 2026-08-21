#include <stdio.h>
#include <unistd.h>
int main(void) {
    printf("hello from musl on Nyx\n");
    fflush(stdout);
    write(1, "direct write works too\n", 23);
    return 0;
}
