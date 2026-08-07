# expect: expects 0 type argument(s)
# Origin parameters are erased from a struct's explicit argument list, so an
# origin-applied struct type cannot be spelled in an annotation; the
# reference-bearing carrier is written bare and its origin is inferred.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def make[o: Origin[mut=True]](out box: RefBox[o]):
    pass

def main():
    print(1)
