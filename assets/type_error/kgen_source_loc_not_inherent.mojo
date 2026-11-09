# expect: attribute 'inline_count' is not an inherent attribute of 'kgen.source_loc'
# `kgen.source_loc` holds only `inlineCount` and its `_type`; the pin
# rejects the attribute spelled as `call_location`'s parameter.
def main():
    var line, col, file_name = __mlir_op.`kgen.source_loc`[
        inline_count=Int(0).__mlir_index__(),
        _type=Tuple[
            __mlir_type.index,
            __mlir_type.index,
            __mlir_type.`!kgen.string`,
        ],
    ]()
    print(Int(SIMDLength(mlir_value=line)))
