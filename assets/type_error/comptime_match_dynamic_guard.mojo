# expect: cannot use a dynamic value in 'comptime if' condition
# A `comptime match` guard is a conjunct of its case's compile-time
# condition; a runtime operand is the pin's `comptime if` rejection.
def main():
    var y = 3
    comptime n = 3
    comptime match n:
        case 3 if y > 2:
            print("three")
        case _:
            print("other")
