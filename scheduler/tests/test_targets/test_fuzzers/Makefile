buffer_overflow: buffer_overflow.c fuzz_buffer_overflow.c 
	mkdir -p build
	clang -g -fsanitize=fuzzer,address --coverage -o build/fuzz_buffer_overflow $^

corpus_test:
	clang -g -fsanitize=fuzzer,address --coverage -o build/fuzz_corpus_creation fuzz_corpus_creation.c

continuous:
	clang -g -fsanitize=fuzzer,address --coverage -o build/fuzz_continuous fuzz_continuous.c

all: buffer_overflow corpus_test continuous

clean:
	rm -rf build