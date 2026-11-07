# expect: '@always_inline("builtin")' does not support this argument convention
# A builtin's result must be trivially register-passable.
@always_inline("builtin")
def name(n: Int) -> String:
    return "a"


def main():
    print(name(2))
