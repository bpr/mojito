# Capturing closure environments through the native capture record:
# immutable and mutable reference captures with write-back, an owned `{var}`
# scalar snapshot re-taken per loop iteration, repeated invocation mutating
# state through the same record, and recursion forwarding the environment.
def main():
    var total = 0
    var step = 3
    def bump() {mut total, imm step}:
        total = total + step
    bump()
    bump()
    print(total)

    var i = 0
    var sum = 0
    while i < 3:
        sum = sum + (lambda {var i} -> Int: i * 10)()
        i = i + 1
    print(sum)

    var depth = 0
    def descend(n: Int) {mut depth} -> Int:
        depth = depth + 1
        if n == 0:
            return depth
        return descend(n - 1)
    print(descend(4))
    print(depth)
