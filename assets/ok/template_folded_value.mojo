# A `comptime for` variable and a value parameter read as runtime values.
# The elaborator folds each to the instance's literal, which keeps the name's
# identity; every instance derives the literal's facts from the checked
# template instead of being checked again.
def bump(x: Int) -> Int:
    return x + 1


def total[n: Int]() -> Int:
    var sum = 0
    comptime for i in range(n):
        sum += i
    return sum


def mixed[n: Int]() -> Int:
    var acc = n
    comptime for i in range(n):
        acc = acc * 2 + i
        acc += bump(i)
        print(i)
    comptime if n > 2:
        return acc
    return n


def pick[flag: Bool]() -> Bool:
    var seen = flag
    comptime if flag:
        return seen
    return flag


def main():
    print(total[0](), total[1](), total[4]())
    print(mixed[0]())
    print(mixed[3]())
    print(pick[True](), pick[False]())
