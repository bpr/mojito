# expect: type mismatch for collection display element
# A collection display default takes its parameter's type as context, so each
# element must fit the parameter's element type: a string is no `Int`.
def count(xs: List[Int] = ["a"]) -> Int:
    return len(xs)


def main():
    print(count())
