# A trait requirement's receiver may be annotated only with `Self`.
# expect: 'self' argument must have type 'Self' in trait method declaration
trait Made:
    def __init__(out self: Int, *, x: Int):
        ...

def main():
    pass
