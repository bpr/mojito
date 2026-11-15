# A trait may be opted out with `not` at most once in a conformance list.
# expect: trait 'Copyable' must be opted out at most once; remove the duplicate 'not'


struct Handle(not Copyable, not Copyable):
    var fd: Int


def main():
    pass
