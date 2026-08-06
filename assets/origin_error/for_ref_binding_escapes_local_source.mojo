# expect: escapes storage
# A for-ref loop binding borrows the iterated collection: returning it from a
# function whose declared origin is a parameter rejects the local borrow.
def first_ref(values: List[Int]) -> ref[
    origin_of(values)._get_owned_interior["element"]
] Int:
    var local: List[Int] = [10, 20]
    for ref x in local:
        return x
    return values[0]


def main():
    var values: List[Int] = [1, 2, 3]
    print(first_ref(values))
