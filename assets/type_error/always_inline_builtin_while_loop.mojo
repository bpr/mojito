# expect: '@always_inline("builtin")' does not support MLIR operation lit.loop
# A builtin body holds no loop.
@always_inline("builtin")
def settle(n: Int) -> Int:
    var m = n
    while m > 3:
        m -= 1
    return m


def main():
    print(1)
