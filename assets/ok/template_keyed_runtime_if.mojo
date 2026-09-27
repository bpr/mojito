# A runtime `if` inside a keyed `def`: in a `comptime for` body, under a
# `comptime if` arm, and after the loop. Each unrolled copy keeps its own
# `if`, its condition folding the loop variable as the copy's other
# statements do, and a local declared in an arm is one binding per copy.
# Every instance derives its facts from the checked template instead of
# being checked again.
def count[n: Int]() -> Int:
    var acc = 0
    comptime for i in range(n):
        if i < 3:
            var d = i * 2
            acc += d
        elif i == 3:
            acc += 10
        else:
            acc -= 1
    return acc


def pick[n: Int](x: Int) -> Int:
    var r = 0
    comptime for i in range(n):
        if x < i and i != 2:
            r += 1
        if x != 0:
            r += 100
    if r > n:
        r = n
    return r


def nested[n: Int](flag: Bool) -> Int:
    var r = 0
    comptime for i in range(n):
        comptime if i % 2 == 0:
            if flag:
                r += i
            else:
                if i > 1:
                    print(i)
    return r


def main():
    print(count[5](), count[2]())
    print(pick[4](2), pick[3](0))
    print(nested[5](True), nested[5](False))
