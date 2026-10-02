def total(*xs: Int) -> Int:
    var sum = 0
    for i in range(len(xs)):
        sum += xs[i]
    return sum


def main():
    print(total(1, 2, 3))
