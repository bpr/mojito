# A runtime `while` inside a keyed `def`: over an `Int` local after a
# `comptime for`, in a
# `comptime for` body with `break` and `continue`, and under a `comptime if`
# arm. Each unrolled copy keeps its own loop, its condition folding the loop
# variable as the copy's other statements do. Every instance derives its
# facts from the checked template instead of being checked again.
def count[n: Int]() -> Int:
    var k = 0
    var acc = 0
    comptime for i in range(n):
        acc += i
    while k < n:
        acc += k
        k += 1
    return acc


def steps[n: Int](limit: Int) -> Int:
    var r = 0
    comptime for i in range(n):
        var j = 0
        while j < limit:
            j += 1
            if j == i:
                continue
            if j > i + 2:
                break
            r += j
    return r


def gated[n: Int](flag: Bool) -> Int:
    var r = 0
    comptime for i in range(n):
        comptime if i % 2 == 0:
            var m = i
            while flag and m > 0:
                r += m
                m -= 1
    return r


def main():
    print(count[5](), count[2]())
    print(steps[3](4), steps[2](1))
    print(gated[5](True), gated[5](False))
