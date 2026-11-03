# expect: invalid call to 'show': an element of 'values' with type 'NW' does not conform to trait 'Writable'; either prove the conformance with 'conforms_to', or add conformance
# A pack collector's element bound refuses an argument by naming the
# collector and the element's type, as upstream reports it for any callee.
struct NW:
    var x: Int

    def __init__(out self):
        self.x = 1


def show[*Ts: Writable](*values: *Ts):
    pass


def main():
    show(1, NW())
