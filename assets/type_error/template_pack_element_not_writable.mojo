# A pack-keyed `def` is checked once from its template, with each element at
# the pack's declared bound: a `Copyable` pack handed to `print` element by
# element is rejected there, before any instance exists, as the pinned Mojo
# rejects it.
# expect: does not conform to trait 'Writable'
def show[*Ts: Copyable](*values: *Ts):
    comptime for i in range(values.__len__()):
        print(values[i])


def main():
    show(1, "two")
