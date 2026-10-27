static I64 add(I64 a, I64 b) { return a + b; }
static I64 mul(I64 a, I64 b) { return a * b; }
typedef I64 (*Op)(I64, I64);
Op ops[2] = {add, mul};
I64 __kai_run_12(void) { return ops[0](3, 4) * ops[1](1, 3); }
