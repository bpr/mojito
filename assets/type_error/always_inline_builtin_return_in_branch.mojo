# expect: '@always_inline("builtin")' does not support MLIR operation lit.return
# A builtin body returns only from its last statement, called or not.
@always_inline("builtin")
def pick(n: Int) -> Int:
    if n > 3:
        return 1
    return n


def main():
    print(1)
