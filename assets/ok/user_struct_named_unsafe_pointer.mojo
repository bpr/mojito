# The pin declares no `UnsafePointer`, so a user struct of that name is an
# ordinary struct. Upstream prints `3`.
@fieldwise_init
struct UnsafePointer(Copyable):
    var n: Int


def show(p: UnsafePointer) -> Int:
    return p.n


def main():
    var p = UnsafePointer(3)
    print(show(p))
