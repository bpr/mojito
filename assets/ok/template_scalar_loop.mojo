# One template occurrence becomes several instance occurrences when a scalar
# `comptime for` unrolls: zero, one, and three trips here, with a nested
# compile-time conditional. Every copy inherits the template occurrence's
# checked facts and names the instance's own `sum`.
def total[n: Int]() -> Int:
    var sum = 0
    comptime for i in range(n):
        comptime if i == 1:
            sum += 10
        else:
            sum += 1
    return sum


def main():
    print(total[0]())
    print(total[1]())
    print(total[3]())
