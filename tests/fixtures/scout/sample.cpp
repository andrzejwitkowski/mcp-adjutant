// comment invoke()
#include <iostream>

class App {
public:
    void run() {
        invoke();
        const char *s = "invoke()";
        this->invoke();
    }
};
