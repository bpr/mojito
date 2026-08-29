# `arg: Optional[Int] = None` — the upstream signature pattern. An omitted-arg
# call materializes the empty Optional by running its constructor (Optional is
# heap-backed), so a method call on the parameter works; an explicitly supplied
# Optional passes through. Optional is prelude-exported on both compilers, so no
# import is needed.

def choose(arg: Optional[Int] = None) -> Int:
    return arg.or_else(-1)

def main():
    print(choose())
    print(choose(Optional[Int](5)))
