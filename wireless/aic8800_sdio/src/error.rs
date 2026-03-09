/// SDIO 错误类型  
#[derive(Debug, Clone, Copy, PartialEq, Eq)]  
pub enum SdioError {  
    /// 命令超时  
    Timeout,  
    /// CRC 校验失败  
    CrcError,  
    /// 数据传输错误  
    DataError,  
    /// 卡未检测到  
    NoCard,  
    /// 不支持的操作  
    Unsupported,  
    /// 通用 IO 错误  
    IoError,  
}  