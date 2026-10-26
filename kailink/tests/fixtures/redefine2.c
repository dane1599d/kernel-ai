I32 call_twice(I32 x);
extern I32 (*saved)(I32);
I64 __kai_run_r1(void) { return call_twice(5) * 100 + saved(5); }
