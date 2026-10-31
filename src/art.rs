//! Album-art decoding: encoded bytes -> small premultiplied-BGRA Direct2D bitmap via WIC.
//! Scaled to the on-screen size up front so only a few KB stay resident.

use windows::{
    Win32::Graphics::{
        Direct2D::{ID2D1Bitmap, ID2D1RenderTarget},
        Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
            WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeMedianCut,
            WICDecodeMetadataCacheOnDemand,
        },
    },
    Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    core::Result,
};

pub fn decode(rt: &ID2D1RenderTarget, bytes: &[u8], px: u32) -> Result<ID2D1Bitmap> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&frame, px, px, WICBitmapInterpolationModeFant)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0,
            WICBitmapPaletteTypeMedianCut,
        )?;
        rt.CreateBitmapFromWicBitmap(&converter, None)
    }
}
