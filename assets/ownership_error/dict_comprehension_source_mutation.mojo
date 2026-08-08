# expect: invalidated interior reference
# The comprehension path shares the statement loop's mapping loan: mutating
# the dict from the element expression invalidates the iteration generation.
def grow(mut d: Dict[StringLiteral, Int], key: StringLiteral) -> Int:
    d[key] = 9
    return len(d)


def main():
    var d = {"a": 1, "b": 2}
    var sizes = [grow(d, k) for k in d]
    print(sizes[0])
