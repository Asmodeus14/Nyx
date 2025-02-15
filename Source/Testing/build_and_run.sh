#!/bin/bash

# Assemble the assembly file
nasm -f elf64 -o test.o test.asm

# Link the object file to create the executable
gcc -nostartfiles -o test test.o

# Run the executable
./test
