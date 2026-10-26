I32 twice(I32 x);
I32 (*saved)(I32) = twice;
I32 call_twice(I32 x) { return twice(x); }
