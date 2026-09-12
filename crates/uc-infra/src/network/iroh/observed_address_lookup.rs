//! 保留原发现服务及结果，只在实际交接处记录安全事实。
use super::connection_diagnostics::candidate_summary;
use futures_util::{stream::BoxStream, Stream};
use iroh::address_lookup::{
    AddressLookup, AddressLookupBuilder, AddressLookupBuilderError, EndpointData, Error, Item,
};
use iroh::{Endpoint, EndpointAddr};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use uc_observability_contract::diagnostics::connectivity::{
    DiscoverySource, LookupObservation, NetworkRecorder,
};

pub(super) struct ObservedAddressLookupBuilder<T> {
    inner: T,
    source: DiscoverySource,
    recorder: NetworkRecorder,
}

impl<T> ObservedAddressLookupBuilder<T> {
    pub(super) fn new(inner: T, source: DiscoverySource, recorder: NetworkRecorder) -> Self {
        Self {
            inner,
            source,
            recorder,
        }
    }
}

impl<T> std::fmt::Debug for ObservedAddressLookupBuilder<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObservedAddressLookupBuilder")
    }
}

impl<T: AddressLookupBuilder> AddressLookupBuilder for ObservedAddressLookupBuilder<T> {
    fn into_address_lookup(
        self,
        endpoint: &Endpoint,
    ) -> Result<impl AddressLookup, AddressLookupBuilderError> {
        Ok(ObservedAddressLookup {
            inner: self.inner.into_address_lookup(endpoint)?,
            local_id: endpoint.id(),
            source: self.source,
            recorder: self.recorder,
        })
    }
}

struct ObservedAddressLookup<T> {
    inner: T,
    local_id: iroh::EndpointId,
    source: DiscoverySource,
    recorder: NetworkRecorder,
}

impl<T> std::fmt::Debug for ObservedAddressLookup<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObservedAddressLookup")
    }
}

impl<T: AddressLookup> AddressLookup for ObservedAddressLookup<T> {
    fn publish(&self, data: &EndpointData) {
        self.inner.publish(data);
        // DNS resolver 的默认 publish 是空实现，不把它记作发布请求。
        if !self.recorder.is_enabled() || matches!(self.source, DiscoverySource::Dns) {
            return;
        }
        let address = EndpointAddr::from_parts(self.local_id, data.addrs().cloned());
        let (summary, fingerprint) = candidate_summary(&address);
        self.recorder.address_publish_requested(
            *self.local_id.as_bytes(),
            fingerprint,
            self.source,
            summary,
        );
    }

    fn resolve(&self, peer: iroh::EndpointId) -> Option<BoxStream<'static, Result<Item, Error>>> {
        let inner = self.inner.resolve(peer)?;
        if !self.recorder.is_enabled() {
            return Some(inner);
        }
        Some(Box::pin(ObservedResults {
            inner,
            observation: Some(self.recorder.lookup(*peer.as_bytes(), self.source)),
        }))
    }
}

struct ObservedResults {
    inner: BoxStream<'static, Result<Item, Error>>,
    observation: Option<LookupObservation>,
}

impl Stream for ObservedResults {
    type Item = Result<Item, Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.observation.is_none() {
            return Poll::Ready(None);
        }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(result)) => {
                if let Some(observation) = &mut this.observation {
                    match &result {
                        Ok(item) => {
                            let (summary, fingerprint) =
                                candidate_summary(&item.to_endpoint_addr());
                            let reported_at = item
                                .last_updated()
                                .and_then(|micros| i64::try_from(micros / 1000).ok());
                            observation.result(fingerprint, summary, reported_at);
                        }
                        Err(_) => observation.error(),
                    }
                }
                Poll::Ready(Some(result))
            }
            Poll::Ready(None) => {
                if let Some(observation) = this.observation.take() {
                    observation.finish();
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
