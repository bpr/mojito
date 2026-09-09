# expect: which names no in-scope Origin parameter
# A def-level Origin parameter cannot bind a struct's explicit origin slot in
# an annotation: only the enclosing struct's origin binders and the builtin
# origins resolve there, so the reference-bearing carrier is written bare and
# its origin is inferred.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def make[o: Origin[mut=True]](out box: RefBox[o]):
    pass

def main():
    print(1)
