all: buffer_overflow.c fuzz_buffer_overflow.c
	mkdir -p build
	clang -g -fsanitize=fuzzer,address -o build/fuzz_buffer_overflow $^