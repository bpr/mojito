# A method's `comptime if` over its own defaulted parameter, called from a
# sibling method that reads none of its struct's parameters, runs: the
# parameter the caller does not forward is one the condition never reads.
struct Flags[T: AnyType, enabled: Bool]:
    def __init__(out self):
        pass

    def describe[verbose: Bool = False](self):
        comptime if verbose:
            print("verbose")
        else:
            print("quiet")

    def report(self):
        self.describe()
        self.describe[True]()


def main():
    Flags[Int, True]().report()
