# An arithmetic `where` operand is retained as a canonical proposition and
# discharged at the application that binds its parameters.
def check[n: Int, m: Int]() -> Int where (n + 1 == m, "increment required"):
    return m


def main():
    print(check[3, 4]())
