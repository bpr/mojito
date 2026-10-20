# expect: does not conform to 'Sized'
# A sequence element is untyped, so a nested sequence pattern cannot test
# its length.
def main():
    var xs: List[List[Int]] = [[1]]
    match xs:
        case [[u]]:
            print(u)
        case _:
            print("other")
