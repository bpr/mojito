# expect: does not conform to trait 'Defaultable'; either prove the conformance with 'conforms_to', or add conformance
# A field type of a symbolic `T` under the loop index is opaque: `T`'s own
# bound says nothing about a field, so constructing one needs a
# `comptime if conforms_to(FT, Defaultable):` arm to prove the trait first.
def defaults[T: AnyType]():
    comptime r = reflect[T]
    comptime types = r.field_types()
    comptime for i in range(r.field_count()):
        comptime FT = types[i]
        print(FT())


def main():
    print("never instantiated")
