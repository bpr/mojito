# A scalar local declared inside a `comptime for` in a compile-time-keyed
# `def`: each unrolled copy of the declaration is a binding of its own, and a
# use names the copy in scope where it sits, so every instance derives its
# facts from the checked template instead of being checked again.
def bump(x: Int) -> Int:
    return x + 1


def total[n: Int]() -> Int:
    var sum = 0
    comptime for i in range(n):
        var step = i
        step += 1
        sum += step
    return sum


def nested[n: Int]() -> Int:
    var acc = 0
    comptime for i in range(n):
        var row = i
        comptime for j in range(2):
            var cell = row + j
            acc += cell
        acc += row
    return acc


def arms[n: Int]() -> Int:
    var acc = 0
    comptime for i in range(n):
        comptime if i % 2 == 0:
            var even = i
            acc += even
        else:
            var odd = bump(i)
            acc += odd
    var tail = acc
    tail += n
    return tail


def shadow[n: Int]() -> Int:
    var x = 100
    comptime for i in range(n):
        var x = i
        x += 1
        print(x)
    return x


def main():
    print(total[0](), total[1](), total[3]())
    print(nested[0](), nested[2]())
    print(arms[0](), arms[1](), arms[4]())
    print(shadow[3]())
