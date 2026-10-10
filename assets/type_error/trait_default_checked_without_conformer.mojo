# A trait default is checked in its trait whether or not anything conforms.
# expect: type mismatch for return: expected Int, found StringLiteral
trait T:
    def d(self) -> Int:
        return "oops"

def main():
    print(1)
