# expect: no field 'w'
# A struct pattern's field must be one of the subject's fields.
@fieldwise_init
struct Point(ImplicitlyCopyable):
    var x: Int
    var y: Int


def main():
    match Point(1, 2):
        case Point(w=1):
            print("w")
