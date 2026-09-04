# expect: is not defined
# A struct without `__is__` does not support the `is` operator.
struct Plain:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

def main():
    var p = Plain(1)
    print(p is None)
