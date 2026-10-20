# expect: must be mutable
# A bare `case x` binds an immutable reference to the subject; assigning to
# it is rejected like any write through an immutable reference.
def main():
    match 1:
        case x:
            x = 2
