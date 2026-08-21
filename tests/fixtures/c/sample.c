#include <stdio.h>

#define MAX_ITEMS 32
#define SQUARE(x) ((x) * (x))

struct point {
    int x;
    int y;
};

union value {
    int number;
    char *text;
};

enum color {
    RED,
    GREEN,
};

typedef struct point point_t;

typedef struct {
    int width;
    int height;
} dims_t;

int add(int left, int right);

int add(int left, int right)
{
    return left + right;
}

static char *greeting(void)
{
    return "hello";
}
