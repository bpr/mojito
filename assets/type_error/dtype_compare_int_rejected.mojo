# expect: operator '==' is not defined for DType and IntLiteral[5]
# `DType.__eq__` takes another `DType`; an integer does not convert.
def main():
    print(DType.int8 == 5)
