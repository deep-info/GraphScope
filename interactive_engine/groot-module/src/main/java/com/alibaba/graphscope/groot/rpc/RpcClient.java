/**
 * Copyright 2020 Alibaba Group Holding Limited.
 *
 * <p>Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file
 * except in compliance with the License. You may obtain a copy of the License at
 *
 * <p>http://www.apache.org/licenses/LICENSE-2.0
 *
 * <p>Unless required by applicable law or agreed to in writing, software distributed under the
 * License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either
 * express or implied. See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.alibaba.graphscope.groot.rpc;

import io.grpc.ConnectivityState;
import io.grpc.ManagedChannel;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.concurrent.TimeUnit;

public abstract class RpcClient implements AutoCloseable {
    private static final Logger logger = LoggerFactory.getLogger(RpcClient.class);

    protected ManagedChannel channel;

    public RpcClient(ManagedChannel channel) {
        this.channel = channel;
    }

    public void checkChannelState() {
        if (channel.getState(true) != ConnectivityState.READY) {
            logger.warn("Current channel State: " + channel.getState(true));
        }
    }

    public void close() {
        channel.resetConnectBackoff();
        this.channel.shutdown();
        try {
            this.channel.awaitTermination(3000, TimeUnit.MILLISECONDS);
        } catch (InterruptedException e) {
            // Ignore
        }
    }
}
