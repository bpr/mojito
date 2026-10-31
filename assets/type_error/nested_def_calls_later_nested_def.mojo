# expect: Undefined variable 'q'
# A nested `def` is bound at its statement, so one above calls it unbound,
# as at the pin ("use of unknown declaration 'q'").


def outer() -> Int:
    def p() -> Int:
        return q()

    def q() -> Int:
        return 5

    return p()


def main():
    print(outer())
