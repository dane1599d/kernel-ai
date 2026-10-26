typedef struct { I32 x, y; } Point;
static I32 dot(const Point *a, const Point *b) { return a->x * b->x + a->y * b->y; }
I64 __kai_run_8(void) {
  Point a = {2, 3}, b = {3, 8};
  return dot(&a, &b);
}
