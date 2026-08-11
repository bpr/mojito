# expect: constraint failed: single digit required
def pick[n: Int]() -> Int where (n > 0, "positive required") where (n < 10, "single digit required"):
    return n


def main():
    var result = pick[12]()
    print(result)
