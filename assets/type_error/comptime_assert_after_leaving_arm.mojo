# expect: function instantiation of `k` failed: constraint failed: never
# A `comptime if` arm that returns ends the instance's path, but the
# assertions after the op are still judged at the instance, as at the pin.

def k[n: Int]() -> Int:
    comptime if n == 1:
        return 1
    comptime assert n == 5, "never"
    return 3


def main():
    print(k[1]())
