# expect: must be mutable
# The payload of a temporary subject is read through a `ref self` getter
# over an rvalue, so even a `ref` payload binding cannot write it.
def main():
    match Optional(String("tmp")):
        case .Some(ref s):
            s += "!"
        case .None:
            pass
