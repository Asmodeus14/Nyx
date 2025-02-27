#include <stdio.h>
#include <Python.h>  // Case-sensitive!

int main() {
    printf("Hello from Nyx OS!\n");
    Py_Initialize();
    PyRun_SimpleString("print('Hello from Embedded Python')");
    PyRun_SimpleString("import sys\nprint(sys.version)");
    Py_Finalize();
    return 0;
}