# A dangling placeholder is non-null identity but never dereferenceable.
# expect: dangling
def main():
    var pointer = Pointer[Int, MutUntrackedOrigin].unsafe_dangling()
    print(pointer[0])
