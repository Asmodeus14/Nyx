#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main() {
    printf("Hello from Standard C Library!\n");
    
    char* mem = malloc(1024);
    if (mem) {
        printf("Malloc works! (Mapped via NyxOS sys_mmap)\n");
        free(mem);
    }
    
    return 42;
}