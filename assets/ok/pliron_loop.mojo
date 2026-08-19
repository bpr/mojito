def sum_below(limit: Int) -> Int:
    var total = 0
    var i = 0
    while i < limit:
        if i % 3 == 0 or i % 5 == 0:
            total = total + i
        i = i + 1
    return total


def compute() -> Int:
    return sum_below(1000)


def main():
    print(compute())
