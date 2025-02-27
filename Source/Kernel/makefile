# Compiler
CXX = /mingw64/bin/g++

# Compiler Flags (use Windows-style paths)
CFLAGS = -IC:/Python312/include -LC:/Python312/libs -lws2_32 C:/Python312/libs/python312.lib


# Source files
SRC = boot.cpp

# Output executable
OUT = boot.exe

# Build Rule
$(OUT): $(SRC)
	$(CXX) $(SRC) -o $(OUT) $(CFLAGS)

# Clean Rule
clean:
	rm -f $(OUT)
