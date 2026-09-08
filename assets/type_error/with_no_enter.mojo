# expect: 'NoEnter' does not implement the '__enter__' method
# A `with` manager must define `__enter__`.
struct NoEnter:
    def __init__(out self):
        pass

    def __exit__(self):
        pass


def main():
    with NoEnter() as f:
        print("x")
