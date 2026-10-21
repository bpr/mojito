# expect: cannot use a dynamic value in 'match' subject
# A `== value` test of a `comptime match` is a compile-time condition; a
# runtime operand is the pin's rejection, worded for the subject.
def main():
    var y = 3
    comptime n = 3
    comptime match n:
        case == y:
            print("three")
        case _:
            print("other")
