# expect: context manager of type 'Both' defines a consuming __enter__ method as well as an __exit__ method; either remove 'var' from its '__enter__' method or remove the '__exit__' method
# A consuming `__enter__` (`var self`) hands the manager to its result, so
# there is nothing left to call `__exit__` on.
struct Both:
    def __init__(out self):
        pass

    def __enter__(var self) -> Self:
        return self^

    def __exit__(self):
        print("exit")


def main():
    with Both() as f:
        print("body")
