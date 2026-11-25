# expect: expects 2 type argument(s), got 3
# A struct's bracket read as a value is its parameter binding, which
# reports the binding's own rejection.
@fieldwise_init
struct pair[a: Int, b: Int](Copyable):
    pass


def main():
    var x = pair[1, 2, 3]
