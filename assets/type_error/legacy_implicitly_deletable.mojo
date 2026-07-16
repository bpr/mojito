# expect: unknown trait 'ImplicitlyDeletable'
struct Legacy(ImplicitlyDeletable):
    var value: Int

def main():
    pass
