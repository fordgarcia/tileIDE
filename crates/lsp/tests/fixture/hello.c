#include <stdio.h>

struct tile {
    int id;
    double ratio;
};

static int area(int w, int h) {
    return w * h;
}

int main(void) {
    struct tile t = { .id = 7, .ratio = 0.5 };
    int a = area(40, 25);
    printf("tile %d ratio %.2f area %d\n", t.id, t.ratio, a);
    return 0;
}
