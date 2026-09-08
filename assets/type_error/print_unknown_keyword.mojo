# expect: invalid call to 'print': unexpected keyword argument 'foo'
# `print` accepts only `sep`, `end`, `flush`, and `file` as keywords.
def main():
    print("x", foo=1)
