# expect: range arguments must share one dtype
# The dtype is inferred from the scalar arguments, which must agree
# (upstream unifies the overload's single `dtype` parameter).
def main():
    for x in range(Int32(1), Int64(5)):
        print(x)
