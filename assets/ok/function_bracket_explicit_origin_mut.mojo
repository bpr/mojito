# An explicit `mut=` keyword binds an origin's mutability binder in a
# function's bracket, ahead of the positional origin it governs, and a
# `False` binds the origin immutably.
def k[mut: Bool, //, o: Origin[mut=mut]]():
    print(mut)


def main():
    var x = 5
    k[mut=True, origin_of(x)]()
    k[mut=False, origin_of(x)]()
    k[origin_of(x)]()
