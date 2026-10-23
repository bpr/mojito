# expect: gone
# A reference to an `@unavailable` function as a value is rejected as a call
# selecting it is.
@unavailable(reason="gone")
def f(x: Int) -> Int:
    ...


def main():
    var h = f
    print(h(1))
