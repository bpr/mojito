# expect: cannot use a dynamic value in 'match' subject
# A `comptime match` subject is a compile-time value; a runtime local is the
# pin's rejection at the subject.
def main():
    var x = 3
    comptime match x:
        case 3:
            print("three")
        case _:
            print("other")
