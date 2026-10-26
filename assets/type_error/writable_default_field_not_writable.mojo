# expect: function instantiation of `__module$std$format$_reflection_write_field_to` failed: constraint failed: Could not derive Writable
# `Writable`'s reflective default writes each field through the field's own
# member, so printing a conformer with a field that is not `Writable` fails
# where the default is instantiated, as at the pin.


struct Opaque(Copyable):
    var p: Int

    def __init__(out self):
        self.p = 1


@fieldwise_init
struct Holder(Writable):
    var o: Opaque


def main():
    print(Holder(Opaque()))
