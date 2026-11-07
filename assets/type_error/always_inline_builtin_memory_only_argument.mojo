# expect: '@always_inline("builtin")' does not support this argument convention
# A builtin's argument must be trivially register-passable, even one its
# body never reads.
@always_inline("builtin")
def twice(n: Int, s: String) -> Int:
    return n + n


def main():
    print(twice(2, "a"))
