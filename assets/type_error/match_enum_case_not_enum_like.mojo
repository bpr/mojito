# expect: cannot resolve inferred member without a contextual type
# Over a subject that is not `EnumLike`, a call-shaped pattern is a struct
# pattern, and a leading-dot head names no type.
def main():
    var i = 3
    match i:
        case .Some(x):
            print(x)
        case _:
            pass
