# expect: invalid call to '__exit__': missing required argument: 'err'
# The error-taking `__exit__(self, err) -> Bool` overload needs the plain
# `__exit__(self)` beside it: the normal exit calls the latter.
struct ErrOnly:
    def __init__(out self):
        pass

    def __enter__(self) -> Int:
        return 1

    def __exit__(self, err: Error) -> Bool:
        return True


def main() raises:
    with ErrOnly() as f:
        print(f)
