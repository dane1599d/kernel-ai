I64 fib(I64 n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
I64 __kai_run_9(void) { return fib(20); }
