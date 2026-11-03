# expect: constraint failed: Could not derive Writable for Holder - member field `o: Opaque` does not implement Writable
# `Writable`'s reflective default asserts each field is `Writable`, with
# `_field_conforms_to_error`'s message, so printing a conformer with a field
# that is not `Writable` fails where the default is instantiated, as at the
# pin.


struct Opaque(Copyable):
    var p: Int

    def __init__(out self):
        self.p = 1


@fieldwise_init
struct Holder(Writable):
    var o: Opaque


def main():
    print(Holder(Opaque()))
