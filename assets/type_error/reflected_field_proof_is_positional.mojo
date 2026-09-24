# expect: does not conform to trait 'Defaultable'; either prove the conformance with 'conforms_to', or add conformance
# A `conforms_to` proof is keyed by the exact element it names: proving the
# first field type `Defaultable` licenses nothing of the field at the loop
# index, which is the one the arm constructs.
def defaults[T: AnyType]():
    comptime r = reflect[T]
    comptime types = r.field_types()
    comptime for i in range(r.field_count()):
        comptime if conforms_to(types[0], Defaultable & Writable):
            comptime FT = types[i]
            print(FT())


def main():
    print("never instantiated")
