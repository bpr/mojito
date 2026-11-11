# A generic alias whose annotation declares a value cannot bind its own type
# binder: the binder's metatype is its trait bound.
# expect: cannot implicitly convert 'Copyable' value to 'Int'
comptime A[T: Copyable]: Int = T


def main():
    print(A[Int])
